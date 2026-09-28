# SP4c — classification clearance reaches the data plane

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** a user without clearance for a classification cannot read a column carrying it, **by any path**.

**Spec:** `docs/en/specs/2026-09-28-classification-clearance-sp4c-design.md` (D0–D5).

**Architecture:** a tag's clearance compiles into a **deny on the tag service** with the cleared principals in `denyExceptions`, through the same compiler and the same transactional recompile seam SP4a built. Tag policies are evaluated first and short-circuit, so a clearance denial wins over any resource grant.

**Tech stack:** Rust workspace (axum, sqlx 0.9 — `query()` takes `&'static str`, never `format!` SQL), Apache Ranger, Cedar, kind cluster `kind-egret`.

## Global Constraints

- **Build isolation is mandatory.** `export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-sp4a/target-check`. Never `.../egret/target`: another session builds the MAIN checkout there, so cargo links the wrong artefacts and reports `E0432 no X in the root` for symbols that plainly exist — a failure that looks exactly like a broken branch and is not one.
- **A command over ~120s is auto-backgrounded by the harness.** Its completion notification **is the result** — read it and continue rather than waiting as though still in the foreground; five agents on this programme lost their turn that way. **Commit before** starting anything long, and never edit files while a long suite is in flight: one agent got seven fake failures because vitest read a half-edited tree.
- sqlx 0.9: `query()` takes `&'static str`. No `format!`-built SQL.
- `COMPILER_GRANT_PREFIX` / `COMPILER_RULE_PREFIX` are defined **once** in `egret-policy/src/store.rs`. A clearance policy needs its own prefix; define it in the same place, never a second copy.
- **The established recompile seam:** affected services expanded through `services_and_tag_services_via`, locked **ascending**, with the `debug_assert!(service_ids.is_sorted())` gate. The expansion happens before the sort because a tag service's id can be lower than its profile's (observed live: 51 vs 52). The sweep sources existing state from `policy.authz_policies` by `service_id` plus the compiler's name prefixes — **never** through the cascading `perm.compiled`. Both were SP4a Criticals.
- A compile failure fails the request and rolls the write back. A stored-but-uncompiled clearance is worse than a rejected one, because the UI shows it as active.
- OpenAPI must satisfy `committed_openapi_json_is_current`; any client change must be exactly `npm run gen:api` output — never bare `orval` — committed exactly as produced. The frontend has **NO prettier**; only `eslint .`.
- Tests: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` only — **never** the dev database `egret`. DB-gated tests skip silently without it, so always `env -u EGRET_ALLOW_DB_SKIP`, and `RUST_TEST_THREADS=4` or the suite exhausts Postgres connections with `PoolTimedOut`, which is not behaviour.
- Every new test is verified by mutation: break the fix, watch that specific test die, and **check *why* it died** — a mutation dying for the wrong reason proves nothing, which has happened three times here. A mutation that **survives** means an assertion is absent, not that the code is extra-safe. And when code **moves**, its assertions do not move with it: re-mutate what a refactor touched. Restore with `cat backup > file` (`cp` is aliased to `cp -i` and silently refuses to overwrite), then confirm `git status --porcelain`.

---

### Task 1: D0 — measure whether a Ranger policy naming a ROLE matches anybody

**This decides the shape of every task after it. Do not write compiler code in this task.**

SP4a's D7 established, by mechanism, that a policy item naming a **group** matches nobody: Trino's coordinator has no `group-provider.properties`, egret's provisioner never writes one, and egret has no usersync — so `getGroups(user)` is empty for every user. That is why the workspace-assignment grant compiles to an enumerated member list.

Nobody has asked the same question about **roles**. Ranger has a `roles` bucket and the perm model already has `PrincipalKind::Role`, so the obvious implementation names the role. But Ranger roles are defined **in Ranger**, and egret pushes nothing to Ranger.

**Files:** Create `docs/en/plans/2026-09-28-sp4c-d0-result.md`.

- [ ] Establish, **by mechanism rather than by a failed query**, whether the Trino Ranger plugin can resolve an egret role name to a set of users. A denied query has many possible causes — a wrong policy, a stale poll, an unrelated deny — so a single failed query is not an answer. Read what egret serves the plugin (`crates/egret-policy/src/ranger/api/download.rs`) and what the plugin asks for; D7's own method is the template.
- [ ] Answer three things explicitly: does anything in egret write Ranger roles; does the plugin have any source of role membership; and if a role name is unresolvable, does a policy item naming it **deny nobody** or **deny everybody** — the two are opposite failure modes and only one is fail-closed.
- [ ] Write the result into `docs/en/specs/2026-09-28-classification-clearance-sp4c-design.md` under D0, replacing the open question with the measurement, then **stop and report**. If the answer is that roles do not resolve, say so plainly; the rest of the plan takes the enumerate-members branch and gains a fourth trigger.

### Task 2: Compile a tag's clearance into a deny

**Files:** Modify `crates/egret-perm-compile/src/`, `crates/egret-policy/src/store.rs` (the new name prefix).

Written for both branches of Task 1, because the plan must not assume its own measurement:
- **If a role name resolves:** the deny names the role in `denyExceptions`.
- **If it does not:** `denyExceptions` carries the role's **members, enumerated at compile time**, exactly as SP4a's workspace-assignment grant does — and Task 3 gains a membership trigger. Read the **stored** `users.username`, never a re-derivation: `ensure_username` appends `-2`/`-3` on collision, and a re-derived slug names the wrong person while compiling, deploying and reporting success. SP4a built a test around exactly this invisible failure.

- [ ] A tag with a non-empty `required_roles` compiles to a tag-service **deny** on that tag's resource, with the cleared principals in `denyExceptions`. Give it its own name prefix alongside the two existing ones, in the one place they are defined.
- [ ] **This amends SP4a's D2** ("tags drive rules only, never access"). Say so in the code, at the site, with the reason — a classification exists to withhold data and a mask still answers the query. A future reader who finds a tag access policy must not conclude it is a bug.
- [ ] A tag gated **only** by `required_groups` compiles to **nothing**, and the skip is reported rather than silent — D7 measured that a group matches nobody, so emitting the deny would produce a control that withholds nothing.
- [ ] Tests, value-level not shape-level: the deny's resource names the tag; the exceptions name the cleared principals (and for the enumerate branch, a **collision-suffixed** username correctly — a user whose username equals their email slug passes whether the fix exists or not); a groups-only tag produces no policy and one reported skip.

### Task 2b: A `role` grant reaches the engine too, or stops claiming to

**Found by D0, in code already merged.** `crates/egret-perm-compile/src/grants.rs:105` compiles `PrincipalKind::Role` straight into the Ranger `roles` bucket, which D0 measured is never populated — so **a role grant matches nobody in the engine**, while `crates/egret-perm/src/eval.rs:160` matches it happily and `explain` therefore reports access the engine does not grant. It is the same divergence D7 found for groups, on a path D7 did not look at, and SP4b's picker offers `role` as an ordinary choice with no warning.

This belongs here rather than in its own increment because SP4c is now exactly the question of which principals reach the engine, and the remedy is the machinery Task 2 builds.

- [ ] Compile a `role` grant to the role's **members, enumerated at compile time**, reusing Task 2's expansion — reading the **stored** `users.username`, never a re-derivation.
- [ ] Reuse Task 3's membership trigger so a role grant does not go stale either. If that means the trigger must cover two consumers, say so in the report rather than duplicating it.
- [ ] Tests: a role grant compiles to its members' usernames and not to a `roles` bucket; `explain` and the compiled policy **agree** about a role-granted subject. That agreement test is the one that would have caught this originally.
- [ ] If anything here cannot be done without widening Task 2's scope, stop and report rather than half-doing it: a role grant that compiles to nothing is at least currently *consistent* with the empty Ranger bucket, whereas a half-migration could produce a policy naming some members and silently omitting others.

### Task 2c: D0b — measure StarRocks, because D0 measured Trino only

Tasks 2 and 2b enforce clearance on Trino. **StarRocks enforces none**, because it has no tag service, and D0's measurement was explicitly Trino-only. That leaves the spec failing its own acceptance criterion: D5 says a user without clearance cannot read the column **by any path**, and a StarRocks engine is a path. User decision (2026-09-28): measure StarRocks, then enforce it there too.

**Files:** Create `docs/en/plans/2026-09-28-sp4c-d0b-starrocks-result.md`.

- [x] Use D0's method: upstream source first, a passive observation to corroborate, and **no conclusion drawn from a denied query alone** — a denial has many possible causes. StarRocks has its **own** Ranger plugin, not Trino's, so nothing measured in D0 carries over.
- [x] Answer three things. **Does a resource-level `deny` with `denyExceptions` actually work in the StarRocks plugin** — SP4a only ever compiled *masks* there, never a deny, so the deny shape is unproven on this engine. **Does the StarRocks plugin resolve a role name**, given it has native roles and may populate `userRoles` where Trino passes a hardcoded `null`. And **which way does an unresolvable principal fail** — deny nobody, or deny everybody, as D0 found for Trino.
- [x] Write the result into the spec beside D0, then **stop and report**. If a resource deny does not work on StarRocks, say so plainly: that is a complete answer and it changes Task 2d from "expand per column" to "there is no mechanism", which the user must then rule on.

**MEASURED 2026-09-28 — `docs/en/plans/2026-09-28-sp4c-d0b-starrocks-result.md`, written into the design spec as D0b.** (1) **The deny shape works.** Every StarRocks check funnels through `rangerPlugin.isAccessAllowed` → the same shared Ranger ACCESS evaluation, the enabling option is `true` for the def the **live** plugin holds (no `options` key; no hidden property set), and egret already serves resource denies to StarRocks that SP4a's gate measured enforcing. `denyExceptions` itself has never been served there — that half rests on the source read, and Task 6 must observe it live. (2) **No role resolution, and not for want of the data:** `setUserRoles` is never called anywhere in `fe/`, `getCurrentRoleIds()` is read for the error message and never inside the `ranger` package, the roles download 404s with `lastKnownRoleVersion=-1` in the live `fe.log`, and the on-disk roles cache — the item D0 could not reach — is an **empty** roles document. (3) **An unresolvable principal denies everybody**, same as Trino, because the deciding code is shared. **Task 2d keeps "expand per column"; the one thing needing a USER RULING is that a clearance on StarRocks can withhold `select` only** — `checkColumnAction` has one caller and it passes `SELECT`, so `insert`/`update`/`delete` are unreachable at column granularity, and buying the write half needs a *table*-level deny that over-blocks untagged columns. A **view** also bypasses the column check entirely.

### Task 2d: Clearance on StarRocks, per column

Written after the measurement, not before it. SP4a already expands a tag-selected **mask** into one resource policy per matched column on StarRocks, because that engine has no tag service — so the machinery exists and clearance reuses it rather than inventing a parallel path.

**D0b (Task 2c) confirms the mechanism exists**, so this task stays "expand per column". Three measured constraints it must respect, all from `2026-09-28-sp4c-d0b-starrocks-result.md`:

- **`select` only.** D1 withholds `select, insert, delete` on Trino, and `engine_accesses` would give StarRocks `select, insert, update, delete` — but `checkColumnAction` has a single caller passing `PrivilegeType.SELECT`, so the other three can never match at column granularity and emitting them is decoration. Emit `select` alone and **report the gap**. Whether the write half is worth a *table*-level deny (which over-blocks every untagged column of that table) is a **USER RULING**, not this task's decision.
- **No dedup.** A row filter dedups per table because it carries no column resource; an access deny *does*, so the emitted count is exactly N.
- **Priority 0, never override.** Without a tag service there is no tag-first short-circuit; deny-beats-allow comes from Ranger's evaluator sort, which holds only at equal policy priority.

- [ ] Expand a clearance into one deny per matched column, using the same expansion SP4a's tag masks use. **No dedup**: unlike a row filter, an access deny carries a column resource, so the count is exactly N.
- [ ] Emit it at **priority 0 / `is_override: false`**. D0b established why this differs from Trino: without a tag service there is no tag-first short-circuit, and deny-beats-allow comes from the engine sorting deny-bearing evaluators first — which holds **only at equal priority**. An override here would break the ordering it looks like it strengthens.
- [ ] **`select` only, by user decision (2026-09-28), because the write half is unreachable at column granularity.** D0b measured that StarRocks' column check has exactly one caller and it passes `SELECT`, so the `insert`/`update`/`delete` that D1 withholds on Trino cannot be withheld per column here; buying them needs a table-level deny that would block every untagged column too. **So a user without clearance can still overwrite a column they cannot read**, and that asymmetry with Trino is real: record it in the report and in Task 5's UI copy. Do not quietly emit a write deny that does nothing.
- [ ] **The expansion ceilings apply and clearance now competes for the same budget.** SP4a refuses rather than truncates, with a per-rule cap and a profile-total cap that refuses only a change pushing a profile over. A clearance on a widely-applied tag can exhaust that budget, so say in the report what a clearance costs against both caps and confirm a refusal still names the profile and the running total.
- [ ] Tests: a clearance on a tag covering N columns produces N denies and refuses past the ceiling rather than truncating; the exceptions carry the stored usernames, collision suffix included.

### Task 3: The triggers, because clearance has three inputs

A compiled clearance depends on the tag's gate, on which columns carry the tag, and — on the enumerate branch — on who holds the clearing role.

- [ ] **Enumerate every writer of each input before fixing any of them**, and put that enumeration in the report. SP4a shipped "a write without its recompile" on **four** separate paths, and the fourth was found only because a later task re-derived the list instead of inheriting it.
- [ ] A change to a tag's `required_roles` / `required_groups` recompiles the affected services, in the same transaction as the write.
- [ ] A tag assignment change already recompiles (SP4a Task 7) — verify it now also covers clearance, rather than assuming it does.
- [ ] On the enumerate branch only: a change to the clearing role's membership recompiles.
- [ ] Tests: each input's write produces the policy change in `policy.authz_policies` **immediately**, with no second write and no manual compile. Assert against the policy table, not the input row — the input row was always written correctly, and that is exactly why this class of defect keeps shipping.

### Task 4: `explain` names a clearance denial

- [ ] A subject blocked by clearance gets its own reason class, naming the tag. Today `explain` would answer `no_grant_matched`, which is the wrong administrative answer: nobody needs to add a grant, somebody needs clearance.
- [ ] Write the reason as an **independent sentence**, not a template parameterised off the others — SP4b's five reason classes are independent for the stated purpose that they cannot collapse into each other, and collapsing is how `no_grant_matched` and `explicit_deny` would both end up reading "denied".
- [ ] `explain` stays blind to hand-written policy, which is stated on its result panel already. It must not be blind to a mechanism this plan adds.

### Task 5: The UI says what is not enforced

- [ ] Where a classification's clearance is set, say that a **group-only** gate is enforced on object access and **not** in the engine, with the measured reason. Neither a group nor a role *name* reaches an engine; what is enforced is the member set egret enumerated at compile time.
- [ ] Say that on a **StarRocks** engine a clearance withholds **reads only** — a user without clearance can still write a column they cannot read (D0b, and the user's ruling). This is the asymmetry with Trino, and an operator who is not told will assume the classification behaves the same on both. Same shape as SP4b's group warning, and the comment says to remove it when a group provider ships.
- [ ] Do not add a control that appears to withhold data while withholding nothing — the failure this programme keeps refusing to ship.

### Task 6: The live gate — the intersection property

The acceptance test is one sentence: **a user without clearance cannot read a tagged column, by any path.** Not that a policy was written.

- [ ] Deploy to kind with **all six** helm values — `oidc.enabled`, `ingress.enabled`, `kernel.launcher=kubernetes`, `backend.replicas=2`, `sharedStore.enabled`, **and `ranger.enabled=true`**. The chart defaults the last to false, and dropping it makes every engine deny every query, which looks exactly like "the policies work" while measuring nothing. That has already happened once here.
- [ ] Verify by the pod, never by helm's report: the serving pods' `imageID` matches the image just loaded, and `EGRET_RANGER__ENABLED=true` read from **inside** a running pod. Confirm the plugin is actually enforcing before trusting any pass — one run found an engine on `access-control.name=allow-all`, enforcing nothing.
- [ ] Tag a column, gate the tag on a role, then as a user **without** that role: read the column through SQL Lab, and read it through a notebook. **Both must be refused**, and the notebook path is the one that made the union visible in the first place. Record the **verbatim** denial each time.
- [ ] As a user **with** the role, confirm the same reads succeed — otherwise the deny is indiscriminate and the measurement cannot tell clearance from an outage.
- [ ] **Measure the view bypass, on StarRocks and on Trino.** D0b read in the StarRocks source that `checkViewPrivilege` and `checkMaterializedViewAction` never descend to base-table columns, which would let a **view over a classified column leak it entirely** — no query has confirmed it. Create a view and a materialized view over the classified column and read through each, as a user without clearance. If the read succeeds, that is a **measured Critical** and the per-column approach does not achieve the goal on that engine; report it as FAILED with the verbatim output rather than adjusting the case. Do the same on Trino, since nobody has checked it there either.
- [ ] **Observe `denyExceptions` actually working**, not just the deny. D0b was explicit that `denyExceptions` has **never** been served to StarRocks and that half of its answer rests on source alone. So the cleared-user read on StarRocks is the first observation of it anywhere, and it must be recorded as such.
- [ ] Remove the clearance and confirm the policy is **reaped** and the read succeeds again. A clearance that cannot be lifted is the one-way door SP4a had to reopen once already.
- [ ] Run as a non-admin subject: auto-bind seeds admin policies granting `*.*.*.*`, so every case run as `admin` passes vacuously.
- [ ] Record actual values, never "worked". Anything that fails is recorded as FAILED with its verbatim output; partial honest results are the success condition, because a run reporting cases it did not observe cannot be falsified.

### Task 7: Say what clearance actually does, because three statements are now measured false

The live gate (`docs/en/plans/2026-09-28-sp4c-live-result.md`) passed four cases and **failed one that defeats the spec's own acceptance criterion**. User ruling (2026-09-28): **state the limit, do not block view creation.** So the remaining work is to make the documents and the UI say what was measured rather than what was assumed — three of the current statements are false in the reassuring direction, which is the failure mode this programme has refused all along.

- [ ] **D5's acceptance criterion is wrong and must be rewritten, not softened.** It says a user without clearance cannot read a tagged column *by any path*. Measured: a **view** over the column returns every classified value to an uncleared user on **both** engines. Trino's DEFINER-rights model fails in both directions at once — an uncleared view owner blocks everybody *including the cleared*, and a cleared owner hands the whole column to the uncleared. Replace the criterion with what clearance achieves: it withholds a **direct read** of the column, on both engines, and nothing more.
- [ ] **Task 5's second UI statement is false as measured.** It says Trino withholds both reads and writes while StarRocks withholds reads only, presented as an asymmetry. The gate measured an uncleared user INSERT **and** DELETE through a live clearance on **Trino**, because the tag sits on a *column* resource while a write check presents a *table* one. There is no asymmetry: **neither engine withholds writes.** Correct the sentence and its test.
- [ ] **Add the view bypass where the clearance is authored**, with both engines named and Trino's two-directional failure stated — an operator who reads only "views are not covered" will not expect a cleared user to be blocked by an uncleared owner.
- [ ] **`explain` must stop trusting a caller-supplied `roles` list.** The gate showed `{"user":"analyst","roles":["sp4c_cleared"]}` returns `allowed` for the very user the engine refuses, so the diagnostic can be told a lie and will repeat it. Resolve the subject's roles server-side and remove the field from the wire — **the precedent is SP4b-1**, which removed `ExplainSubject.workspaces` for exactly this reason rather than leaving a field that looked authoritative. A "what-if" tool would be a different product; D9 defines this one as answering why a subject **is** allowed or refused.
- [ ] Tests: each corrected statement is asserted **present** (wording is what a later refactor drops silently), and `explain` ignores or rejects a caller-supplied role rather than honouring it. Verify by mutation, and check why each died.

**Recorded as a follow-up, not fixed here:** `GET /api/control-plane/policies/{id}` returns the Ranger shape while `PUT` expects the editor shape, so a read-modify-write round trip **silently drops every `policyItem` and still answers 200**. That is a data-loss defect in the policy editor API, found while repairing the cluster, and it belongs to whoever owns that surface.
