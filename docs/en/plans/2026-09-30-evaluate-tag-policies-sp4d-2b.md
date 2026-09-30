# SP4d-2b — evaluate the tag policies the engine is enforcing

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** lift the `Undecidable` that SP4d-2a shipped for any profile whose tag
service carries a policy, by evaluating tag policies the way the engine does.

**Architecture:** the caller resolves the tag set for a resource — using the same
`RangerDefaultPolicyResourceMatcher` the crate already ports, with the tag
assignment in the policy position — and hands it in. The crate gains a tag phase
before the resource phase, with Ranger's three-way priority arbitration between
them.

**Spec:** `docs/en/specs/2026-09-30-evaluate-tag-policies-sp4d-2b-design.md`

## Open item for the whole-branch review — raised by Task 5, not closed

**Is the composed ACCESS path finished?** Task 5's gate found **five** defects in
code that 127 unit tests and three review rounds had passed, and the fifth only
became visible once the fourth was fixed. Its implementer said plainly it does not
believe the path is done, and named the one audit it could not close:
`evaluatePolicyItems`' **`accessTypeResults` map lives in the request context and
spans evaluators** — which is how the tag phase turned out to arbitrate by
`policyPriority` after the crate's prose asserted it did not (defect T4). That same
map is live on the **resource** path too, where the crate models a *different*
mechanism and is green over 4000 cases. The two coincide because the engine loop
shuts out lower-priority evaluators first — but that is **one seed at one size, not
a proof**, and it is the audit to do before anyone models T4 or T5 rather than
refusing them.

## Global Constraints

**The frontend gates are part of every task that changes a wire type, not only of
the frontend tasks.** `npx tsc --noEmit` and `npx eslint .` in `frontend/` are
required alongside the Rust gates whenever a task adds or changes an
`UndecidableCause` (or any other mirrored variant), because the generated client
changes and `ExplainAccess.tsx`'s `causeSentence` switch is deliberately
exhaustive — a new variant makes it fail to compile, which is the guard working.
Task 2 added three causes and shipped with `tsc` red; it went unnoticed because
`tsc` was not on this plan's verification list, and the controller's own
hand-verification of that task inherited the same omission. A gate that is not on
the list is a gate nobody runs. Tasks 6 and 7 touch that page directly and must
run both.

- **`egret-ranger-eval` stays dependency-pure:** `serde` + `serde_json` only. No
  utoipa, no sqlx, no I/O in library code. Wire DTOs live in the route crate.
- **The tag set is an INPUT.** The crate never queries a database. The resolution
  lives in `egret-policy`; the evaluation lives in the crate.
- **Match tag policies on `tags.name`, never `tags.key`.** They are different
  columns and live they differ (`sp4cgate` vs `sp4cgate2`). `tags.key` stays on
  `explain`'s attribution surface. One function must not return both.
- **Never dedupe a tag by name.** `RangerTagForEval`'s `equals`/`hashCode` include
  `matchType`, and MASK evaluators are ordered by match type, so the same tag at
  two levels produces a different masking answer. Deduping silently reorders masks.
- **Evaluate only column- and table-level attachments**, and **refuse by shape**
  rather than assume: a tag service resource with fewer than three resource
  elements did not come from `from_assignments`, so answer `Undecidable` naming it.
- **Gate tag relevance on the service-def's `contextEnrichers`, never on
  `EngineType`.** Both defs are consulted; `None` on either refuses. SP4d-2a
  proved the single-def version fail-open.
- **Do NOT narrow `Unmodelled::ServiceConfig`.** Super-users short-circuit before
  any policy at `Integer.MAX_VALUE` priority.
- Never run tests against the dev DB `egret`; use `egret_test` with
  `env -u EGRET_ALLOW_DB_SKIP` and `RUST_TEST_THREADS=4`.
- Never use `git stash`. Never override the git author.
- Frontend: **no prettier** (`eslint .` only); `var(--token)` never
  `hsl(var(--token))`; regenerate with **`npm run gen:api`**, never bare `orval`.
- The OpenAPI drift gate must be run as
  `cargo test -p egret-server --lib committed_openapi_json_is_current` —
  **without `--lib` it reports "0 passed, 1 filtered out" and exits 0.**
- `cp` is `cp -i` and `rm` is `rm -i`, both silently no-op: restore with
  `cat backup > file`, delete with `command rm -f`. After a substitution, verify
  the file changed before believing a SURVIVED.
- zsh has no `${PIPESTATUS[0]}`; capture exit codes from the command itself.
- **A command over ~120s is auto-backgrounded and its notification IS the finished
  result.** Commit before running anything long; two tasks on the predecessor
  branch stalled on this and had to be taken over. Leave `git status` clean, not
  merely `git diff` clean.

---

### Task 1: Resolve the tag set the way the enricher does

**Files:**
- Modify: `crates/egret-policy/src/store.rs`
- Test: `crates/egret-policy/tests/tag_resolution.rs`

**Interfaces:**
- Produces: a resolution returning, per matched attachment, the tag **name** and
  its `MatchType`, with no deduplication.

- [ ] **Step 1: Build the fixture no live data provides.** Every one of the six
live tag assignments is **column-level**. The table-level arm — the one that
produces `ANCESTOR` — is representable in the schema (`column_name = ''`) and
**exercised by nothing**. Create it: a tag attached at table level, queried at
column level.

Also build the case D4 exists for: **the same tag attached at both the table and
the column**, which must come back **twice** with different match types.

- [ ] **Step 2: Write the failing tests.**
1. Column attachment, column request → one entry, `SELF`.
2. Table attachment, column request → one entry, `ANCESTOR`.
3. Both → **two** entries, `SELF` and `ANCESTOR`, not one.
4. A column attachment with a **table-level** request → **excluded** (that is
   `DESCENDANT`, which the enricher rejects). Returning it would fail closed on
   `explain` and still disagree with the engine.
5. The returned identifier is `tags.name`, not `tags.key` — assert against a
   fixture where they **differ**, or the test cannot tell.

- [ ] **Step 3: Run them, watch them fail. Step 4: Implement. Step 5: Confirm green.**

- [ ] **Step 6: Mutation-check the two that hide.** Dedupe by name and confirm test
3 dies. Return `key` instead of `name` and confirm test 5 dies. Both are silent
fail-opens; a test that does not die here is not testing them.

- [ ] **Step 7: Commit.**

---

### Task 2: Carry the match type into the crate, and evaluate a tag policy

**Files:**
- Modify: `crates/egret-ranger-eval/src/{model.rs,eval.rs}`
- Test: `crates/egret-ranger-eval/tests/tag_eval.rs`

**Interfaces:**
- Consumes: Task 1's shape.
- Produces: a tag phase that evaluates `tagPolicies.policies` against
  `{tag: <name>}` using the block's own `serviceDef`.

- [ ] **Step 1: Read what already exists before designing.** `resource.rs`'s
`match_type` / `matching_hierarchy` / `init` are the enricher's algorithm, already
differentially tested against the real Java engine. This task **must not
reimplement matching** — it calls it with the tag's resource in the policy
position. If you find yourself writing a matcher, stop and re-read.

- [ ] **Step 2: Write the failing tests**, using the real captured fixture
`crates/egret-ranger-eval/tests/fixtures/tag-policies-live.json` — three real tag
policies, ids 8799 (mask), 8805 (row filter), 8839 (deny-capable access):
1. A tag in the resolved set matching policy 8839's `tag` value ⇒ a deny.
2. A tag NOT in the set ⇒ that policy cannot decide.
3. `ANCESTOR` is admitted on the tag path (it is not, for a resource policy).
4. `DESCENDANT` and `NONE` are not admitted.
5. The tag def, not the resource def, drives matching — `tag.json` has one
   resource, `excludesSupported: false`, and **`ignoreCase: false`**, so tag
   matching is case-SENSITIVE where resource matching is not.

- [ ] **Step 3: Run, watch fail. Step 4: Implement. Step 5: Confirm green.**

- [ ] **Step 6: Commit.**

---

### Task 3: Two phases, and the three-way arbitration

**Files:**
- Modify: `crates/egret-ranger-eval/src/eval.rs`
- Test: `crates/egret-ranger-eval/tests/tag_arbitration.rs`

**Interfaces — what Task 2 built and what it deliberately left to you:**
- Task 2's three tag-phase functions **do not run `request_preamble`**: no
  super-user short-circuit, no envelope refusals. That was deliberate, on the
  premise that **you are their only caller**. So two things are on you: run the
  preamble exactly once, before the tag phase, and confirm nothing else in the
  crate or outside it can reach those functions without it. If any can, the
  boundary is wrong — tighten it to `pub(crate)` rather than adding a second
  preamble call.
- `TagDecision` carries `priority`; the arbitration below reads it. You do not
  need to recompute a tag policy's priority.
- **Two facts in an earlier brief of mine were wrong; Task 2 corrected them by
  running the jar, and the corrections are already in the code. Do not
  re-derive them from my prose:** `NAME_COMPARATOR` is priority DESC → `hasDeny`
  → name (**not** the policy name alone), and `MATCH_TYPE_COMPARATOR`'s order was
  obtained by RUNNING it, because the enum's declaration order differs from the
  comparator's intent — a rank taken from `ordinal()` reorders masks. If your
  work needs either ordering, call Task 2's code; do not write a second copy.
- `EvalOrderAmbiguous` does **not** arise on the tag path: both tag comparators
  are fully reproduced, so there is no tie the crate cannot order. Do not add a
  refusal for it here.

- [ ] **Step 1: Verify the arbitration in both jars before implementing it.** The
shape, to be confirmed rather than trusted: tag policies run first; the tag
verdict's `isAccessDetermined` is **cleared** so resource policies can still
decide; it is restored afterwards if nothing else determined. Then per resource
evaluator, before `evaluate` is called:
- tag **deny** ⇒ a resource policy runs only if **strictly higher** priority
  (`>=` stands the tag verdict);
- tag **allow** vs a resource **access** policy ⇒ same-or-higher still runs
  (`>` stands it);
- tag **allow** vs a resource **mask/row-filter** policy ⇒ `>=`.

- [ ] **Step 2: Write the failing tests**, one per arm, plus: a tag deny that a
higher-priority resource allow reopens; a tag allow that a same-priority resource
deny closes; and the case where nothing else determines, so the tag verdict is
restored.

- [ ] **Step 3: Run, watch fail. Step 4: Implement. Step 5: Confirm green.**

- [ ] **Step 6: Mutation-check each asymmetry.** Swap `>` for `>=` on the access
arm and confirm a test dies; swap the other way on the mask arm and confirm a
different test dies. The two asymmetries are one character apart and are the
easiest thing in this package to get backwards.

- [ ] **Step 7: Prove the preamble runs exactly once, and cannot be skipped.**
Write a test that a super-user is short-circuited on a request that ALSO has tag
policies that would otherwise deny — if the tag phase ran before the preamble,
that test fails. Then enumerate every caller of the three tag-phase functions
(`rg` for each name across the whole workspace, tests included) and record the
list in your report. The claim to support is not "I added a call" but "no path
reaches them without it".

- [ ] **Step 8: Commit.**

---

### Task 4: The refusals that keep this honest

> **Correction recorded after implementation (2026-10-01).** This task's brief
> placed D2's "fewer than three elements" rule beside D9's drop as though they
> were one thing. They are **different rules**, and the jar says so:
> `hierarchyHasAllResources` is a **SUBSET** test, not a covering one, so Ranger
> **KEEPS** `{catalog}` and `{catalog,schema}`. Conflating them would make the
> crate answer where Ranger drops and drop where Ranger answers. The distinction
> is pinned on the real `trino.json` by `EgretTagServiceResourceValidationTest`
> (ten key sets, identical at both versions) and mirrored by ten rows in
> `tests/tag_refusals.rs`. Item 2 of the brief also needed no new mechanism: tag
> attributes reach a decision only through `${{ … }}` expressions and
> `conditions`, and every one of those surfaces already refused — what was
> missing was an assertion that those refusals reach the tag path.

**Inherited dependency — read before you model tag attributes.** The crate's tag
identity is `(name, match_type)`. That is a **coarsening** of Ranger's
`RangerTagForEval`, whose `equals`/`hashCode` also take in `attributes`,
`validityPeriods` and `options`. It is harmless while attributes are unmodelled,
because nothing can tell two same-named tags apart anyway. The moment this task
models tag **attributes**, that identity must widen in the same commit — otherwise
two tag members Ranger keeps apart collapse into one, silently, and the dedup that
looks correct today starts dropping a member. Widen the identity or refuse the
payload; do not model attributes while leaving the identity narrow.

**Files:**
- Modify: `crates/egret-ranger-eval/src/{model.rs,eval.rs}`
- Test: `crates/egret-ranger-eval/tests/tag_refusals.rs`

- [ ] **Step 1: Write the failing tests.**
1. A tag carrying `options` or `validityPeriods` ⇒ `Undecidable`. egret cannot
   emit either, so this is provably inert **today** — and asserting it is what
   stops a hand-written `/tags/download` shim or a future feature from silently
   changing the answer.
2. A matched tag policy referencing a tag **attribute** ⇒ `Undecidable`. Unlike a
   validity period, an absent attribute expands to an **empty string** — a value,
   and a different one from Ranger's. This must be a refusal, not an omission.
3. A tag service resource with **fewer than three** resource elements ⇒
   `Undecidable` naming the shape. That payload did not come from
   `from_assignments`, so the schema/catalog narrowing does not apply to it.
4. A tag policy carrying `validitySchedules` or `conditions` ⇒ `Undecidable`. The
   existing refusal must be reachable on the tag path, not only the resource path.
5. **Control:** `Unmodelled::ServiceConfig` still refuses. Assert it explicitly so
   a future narrowing cannot take it by accident.

- [ ] **Step 2: Run, watch fail. Step 3: Implement. Step 4: Confirm green.**

- [ ] **Step 5: Also reproduce D9**, measured in the enricher: a service resource
whose key set validates under no policy type is **dropped**, and its
`resourceToTagIds` entry removed with it — and `isEnabled` on a service resource
is **never consulted**. Test both.

- [ ] **Step 6: Commit.**

---

### Task 5: Extend the differential gate, and re-measure the refusal rate

**Files:**
- Modify: `crates/egret-ranger-eval/tests/differential.rs`
- Modify: `tests/ranger/src/test/java/EgretCaseRunner.java`
- Modify: `tests/ranger/README.md`

- [ ] **Step 1: Generate tag cases** through the existing case-runner protocol, at
**both** plugin versions. Axes: a tag in the set and not; each admitted match type;
the deny-capable access policy, the mask and the row filter; several tag policies
at one priority; and the same tag at two levels with different match types.

- [ ] **Step 2: Give every new axis a coverage counter and a floor**, as the
existing 45 axes have. A corpus that silently stops producing a shape is a gate
that silently stops being a gate — and it fails green.

- [ ] **Step 3c: Measure the `trie_excludes` over-refusal rate.** Task 3 added a
three-valued candidate rule, and its third value refuses. It will over-refuse on
downloads whose resources it cannot prove candidacy for.

**Do not carry forward Task 3's stated bound — a re-review falsified it.** The
report claimed "egret's compiler emits literal values, so nothing egret authored
reaches it". That holds only by accident: `scope_to_resources`'s `Access` arm fills
unnamed levels with `*`, and its `Mask` arm does not. Any download egret did **not**
author — an ordinary table-level grant, which is exactly what `explain` reads off a
live plugin — lands in the refusing branch. So the population that hits this is not
exotic hand-written policy; it is ordinary policy from live engines, which makes the
rate matter more, not less. Task 3 could not measure it because the differential
corpus generates no `tagPolicies` block, so `allowed` is never seeded before the
loop and the rule is **unreachable in the corpus as it stands**. Once this step's tag cases
exist the rule becomes reachable; measure how often it fires, and report it beside
the two `EvalOrderAmbiguous` rates. A refusal nobody has ever measured the cost of
is a refusal nobody can triage.

- [ ] **Step 3: RE-MEASURE `EvalOrderAmbiguous` on the RESOURCE path AND the
COMPOSED path.** This step has been narrowed once and widened once; here is where
it stands. Task 2 established the refusal does **not** apply to the tag phase in
isolation — both tag comparators are fully reproduced, so there is no tie the
crate cannot order. Task 3 then found it **does** arise once the phases are
composed on ACCESS: `policyPriority` is live, so under a tag deny an allowing
resource policy raises the bar and shuts out the rest of its own band, and which
of an allowing and a denying band member runs first is then decided by
`getEvalOrder()`, which this crate does not reproduce. So measure two rates, and
report them separately: the resource-path rate (was 3.4–4.0%) and the rate for
**tag-carrying composed cases**, which nothing has ever measured because nothing
in the corpus generates a `tagPolicies` block yet. Generating those cases is part
of this step, not a precondition for it.

- [ ] **Step 4: Observe the gate failing.** Break one tag rule — admitting
`DESCENDANT` is the sharpest, since it is the fail-open direction — confirm the
differential catches it, restore, verify byte-identical.

- [ ] **Step 5: Commit.**

---

### Task 6: `required_roles` becomes an `Option`

**Files:**
- Modify: `crates/egret-perm-compile/src/routes.rs`
- Modify: `openapi.json`, `frontend/src/api/generated/**` (by generator only)
- Modify: `frontend/src/routes/governance/ExplainAccess.tsx` and its test

- [ ] **Step 1: Make the ambiguity unrepresentable.** The field currently conflates
three facts, and the page re-derives the distinction from a **second** field
(`attribution.tag === null`) because the first cannot carry it. The
`.unwrap_or_default()` in the handler is the exact place a missing row becomes an
empty list.

Be honest about the gain: this is **3→2**, not 3→3. "Declares only groups" and
"no `classification_policies` row" both legitimately produce `Some([])`.

- [ ] **Step 2: Regenerate, in order.** `cargo run -q -p egret-bin -- openapi > openapi.json`,
then the drift gate **with `--lib`**, then `cd frontend && npm run gen:api`.
Never hand-edit either generated artefact.

- [ ] **Step 3: Rewrite the page's three branches** to read one field instead of
two, and update the three fixtures. Keep three branches — the wire now
distinguishes one of them properly and the other two honestly stay together.

- [ ] **Step 4: Commit.**

---

### Task 7: Wire it up — the refusal lifts

**Files:**
- Modify: `crates/egret-perm-compile/src/routes.rs`
- Modify: `frontend/src/routes/governance/ExplainAccess.tsx` and its test

- [ ] **Step 1: Pass the resolved tags.** The handler currently sends
`resolved_tags: None` deliberately, because egret's resolution was not
enricher-equivalent. Task 1 makes it equivalent for the two levels egret can
express; pass it, and keep the shape refusal for anything else.

- [ ] **Step 2: Write the failing test that matters most:** a profile whose tag
service carries a real policy now **answers** instead of refusing. Use the
two-profile fixture.

**Then the sibling — and read this before you write it, because the obvious
version is a lie.** Task 4 measured that two of its refusals, and the D9 drop,
are **unreachable by any code path today, and stay unreachable after this task**.
A review then made the claim stronger: there is **no producer of
`egret_ranger_eval::eval::ResolvedTag` anywhere outside the crate's own tests**,
and the one production `AccessRequest` (`routes.rs:1529`) passes
`resolved_tags: None`. Note the type carefully — my first version of this
paragraph named `egret_policy::ResolvedTag`, which is a **different, two-field
type** returned by `resolved_tags_for_resource_via` over `catalog_tag_assignments`
(no `options`, no `validityPeriods`, no `attributes`). Your task is to build the
mapping between those two types, so do not go looking for the wrong one. So a test that drives the handler end-to-end
**cannot** produce a payload whose shape egret could not have produced — it will
pass while exercising nothing, and it will look like the strongest test in the
package.

Write it instead as a unit test on the handler's
`egret_policy::ResolvedTag → egret_ranger_eval::ResolvedTag` mapping, or on a
hand-built attachment, and **say in the test's name or its comment that the input
cannot arise from the live path today**. Making these refusals genuinely live
means giving the crate a reader for the `/tags/download` payload; that is a
feature package, not this step. Do not quietly widen scope to reach them, and do
not write a green that implies coverage this code path does not have.

- [ ] **Step 1b: The empty-string row is a property of the REQUEST, not the query.**
Task 4's re-review measured this and it is a trap for your mapping:
`resolved_tags_for_resource_via(…, "", "", "")` **does** return a `('','')` row.
Nothing in the SQL prevents it — the predicate is an exact
`schema_name = $2 AND table_name = $3`, so an empty request matches rows written
empty. So do not write, anywhere, that such a row "cannot be returned"; state the
mechanism instead — which caller supplies the three values, and what constrains
them there. If nothing constrains them, that is the finding, and the mapping is
where it has to be handled.

- [ ] **Step 2b: Carried from Task 4 — make enrichment-before-phase
unrepresentable, here rather than earlier.** Nothing structurally forces the tag
enrichment to run before `tag_candidates`; it is a convention held at two call
sites, and the two-guards-must-agree shape has produced a defect in this package
three times. The fix is a newtype on `AccessRequest::resolved_tags`, which breaks
every construction site — including the mapping you are writing in Step 1. Task 4
deliberately left it rather than churn those sites twice. Do it with your mapping,
in the same commit, or say why it is worse here than a convention.

- [ ] **Step 3: The page's copy changes meaning.** SP4d-2a's blast-radius sentence
said a tag policy anywhere makes every verdict `Undecidable` installation-wide.
That stops being true for the cases this package handles. **Rewrite it to what is
now true, do not delete it** — the shape refusal and the attribute refusal remain,
and a reader needs to know which is which.

- [ ] **Step 4: Confirm `explain_writes_nothing` and
`policy_read_permits_the_reads_and_refuses_the_writes` still pass unchanged.**

- [ ] **Step 5: Commit.**

---

### Task 8: Ask the engine that is actually enforcing

**Ruling carried from Task 3 — go through the composed evaluator, not the phase.**
`evaluate_tag_data_mask` and `evaluate_tag_row_filter` are `#[cfg(test)]`, and they
stay that way. Task 3 tightened that boundary because the phase functions do not
run `request_preamble`, so a caller reaching them directly gets no super-user
short-circuit and no envelope refusals — which is the fail-open Task 2's hand-off
predicted and Task 3 found live, at 48 call sites. Do **not** un-`cfg` them to make
a live case easier to drive: compare the engine against `evaluate` / `evaluate_sub`,
the same entry point the explain route uses, so what you measure is what an operator
is shown. If a live case genuinely cannot be expressed through the composed entry
point, that is a finding about the entry point — report it, do not route around it.

**Files:**
- Create: `docs/en/plans/2026-09-30-sp4d-2b-live-result.md`

**This is the task the package exists for.** Trino profile `sp3c2-trino-live` has
**ingested** three tag policies — its plugin cache holds them at `policyVersion 22`,
matching egret's store — including a deny-capable clearance. Until now `explain`
answered `Undecidable` for it. This task asks whether the answer it now gives is
the answer the engine gives.

- [ ] **Step 1: Predict first, in writing, and commit the predictions separately**
from the observations — as the predecessor's browser gate did, so the method is
auditable. A prediction recorded after the observation proves nothing.

- [ ] **Step 2: The cases.**
1. A column carrying the live `pii` tag, against the live mask tag policy (8799) —
   read the masked value back.
2. The same column against the row-filter tag policy (8805) — **at table
   granularity**, because `rowFilterDef.resources` names no `column`.
3. The clearance tag policy (8839): an uncleared subject denied, a cleared one not.
4. **A table-level tag attachment**, which no live data has and Task 1's fixture
   invented — create it, and confirm the engine agrees it is an `ANCESTOR` match.
   This is the arm derived from bytecode and never observed; it is the single most
   valuable measurement in the package.
5. The same tag at both table and column level, to confirm the mask ordering the
   `MATCH_TYPE_COMPARATOR` implies.

- [ ] **Step 3: Remember the poll.** egret IS the Ranger Admin and the plugin polls
every 30 seconds. Show evidence the plugin holds the version you predicted from —
a `policyVersion`, a cache dump, a log line. Not "waited 40 seconds".

- [ ] **Step 4: A disagreement is a FINDING.** Do not tune the evaluator, the
resolution or the page to match a prediction. Work out which of the three is
wrong — evaluator, resolution, or this package's reading of Ranger — and say so.

- [ ] **Step 5: Commit the result doc.**

---

## Definition of done

- A profile whose tag service carries a policy gets an **answer**; one whose
  payload shape egret could not have produced still refuses, naming the shape.
- Tag matching uses `tags.name`; the same tag at two levels appears twice with
  different match types and is not deduped.
- The three-way arbitration is implemented and each asymmetry is mutation-pinned.
- Validity periods, tag attributes, tag-policy constructs and the payload shape all
  refuse, and `ServiceConfig` still refuses.
- The differential gate covers tag cases at both plugin versions, every new axis
  has a floor, the gate has been **observed failing**, and `EvalOrderAmbiguous` is
  **re-measured** rather than carried forward.
- `required_roles` is an `Option`, with the honest 3→2 stated.
- The live gate records a prediction made before each observation, including the
  table-level `ANCESTOR` arm that no live data previously exercised.
- All crate suites green against `egret_test`; `cargo check --workspace`, fmt,
  clippy, `tsc`, `eslint`, the page's vitest file alone, and the drift gate with
  `--lib` all green.
