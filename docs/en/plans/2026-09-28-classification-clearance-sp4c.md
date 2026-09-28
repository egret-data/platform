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

- [ ] Where a classification's clearance is set, say that a **group-only** gate is enforced on object access and **not** in the engine, with the measured reason. Same shape as SP4b's group warning, and the comment says to remove it when a group provider ships.
- [ ] Do not add a control that appears to withhold data while withholding nothing — the failure this programme keeps refusing to ship.

### Task 6: The live gate — the intersection property

The acceptance test is one sentence: **a user without clearance cannot read a tagged column, by any path.** Not that a policy was written.

- [ ] Deploy to kind with **all six** helm values — `oidc.enabled`, `ingress.enabled`, `kernel.launcher=kubernetes`, `backend.replicas=2`, `sharedStore.enabled`, **and `ranger.enabled=true`**. The chart defaults the last to false, and dropping it makes every engine deny every query, which looks exactly like "the policies work" while measuring nothing. That has already happened once here.
- [ ] Verify by the pod, never by helm's report: the serving pods' `imageID` matches the image just loaded, and `EGRET_RANGER__ENABLED=true` read from **inside** a running pod. Confirm the plugin is actually enforcing before trusting any pass — one run found an engine on `access-control.name=allow-all`, enforcing nothing.
- [ ] Tag a column, gate the tag on a role, then as a user **without** that role: read the column through SQL Lab, and read it through a notebook. **Both must be refused**, and the notebook path is the one that made the union visible in the first place. Record the **verbatim** denial each time.
- [ ] As a user **with** the role, confirm the same reads succeed — otherwise the deny is indiscriminate and the measurement cannot tell clearance from an outage.
- [ ] Remove the clearance and confirm the policy is **reaped** and the read succeeds again. A clearance that cannot be lifted is the one-way door SP4a had to reopen once already.
- [ ] Run as a non-admin subject: auto-bind seeds admin policies granting `*.*.*.*`, so every case run as `admin` passes vacuously.
- [ ] Record actual values, never "worked". Anything that fails is recorded as FAILED with its verbatim output; partial honest results are the success condition, because a run reporting cases it did not observe cannot be falsified.
