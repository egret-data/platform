# SP4c — classification clearance must reach the data plane

**Status:** design, approved in principle 2026-09-28.
**Follows:** SP4a (the permission model, #596) and SP4b (its UI, #600/#601).

## The defect this exists to close

A classification's clearance is enforced when someone **clicks the object** and not enforced when they **query the column**.

Cedar compiles a tag's `required_roles` / `required_groups` into a `forbid` policy (`crates/egret-auth/src/pdp/mod.rs:159-206`, template at `pdp/templates.rs:77-98`) which runs on **control-plane object actions** — opening a dashboard, a dataset. The permission model gates the **data-plane query** through Ranger. SP4a's Rules select by tag but read only the tag's `key`: a grep finds **zero** references to `required_roles` or `required_groups` anywhere in `egret-perm` or `egret-perm-compile`.

So the two mechanisms can disagree about the same column, and nothing reconciles them. A user whom Cedar forbids from opening a tagged dataset can read the same data through SQL Lab or a notebook if they hold a read grant. **Effective access is the union of whichever path the user takes, not the intersection.**

**User decision (2026-09-28): clearance must reach the data plane.** A tag requiring role X means that column is refused at the engine to anyone without X. Effective access becomes the intersection.

## D0 — MEASURED: a Ranger policy item naming a role matches nobody, and a role-named `denyExceptions` denies everybody

**Measured 2026-09-28. Full working: `docs/en/plans/2026-09-28-sp4c-d0-result.md`.** Method is D7's: upstream source (Apache Ranger `3.0.0-SNAPSHOT` at `0274a6a0bc96`, Trino's built-in `plugin/trino-ranger`), corroborated by a passive log read of the running coordinator. No query was run and nothing on the cluster was changed, because a denied query has many possible causes.

**Roles are exactly as unresolvable as groups, for a different reason, and they fail in the opposite direction.**

1. **Nothing in egret can serve a Ranger role.** The plugin fetches roles from `/service/roles/download/<service>` (`RangerRESTUtils.java:47`, `RangerAdminRESTClient.getRolesIfUpdated:207`) — a separate document with its own version counter, fetched by `RangerRolesProvider`. egret's `/service` surface is six GET routes (`ranger/routes.rs:15-31`, `api/crud.rs:18-24`) and **none of them is a roles route**, so the poll 404s. A role definition cannot ride in the policy payload either: the `ServicePolicies` envelope (`egret-policy-api/src/dto.rs:21-42`) has no roles member, and neither does Ranger's. No writer exists anywhere in `crates/` or `deploy/`.
2. **The plugin has no other source of membership.** Trino's built-in plugin constructs every request with `userRoles` hardcoded `null` (`RangerTrinoAccessRequest.java`, fifth `super(...)` argument) and no-ops every SQL-role hook (`RangerSystemAccessControl.java:684-702`). `null` normalises to an empty set (`RangerAccessRequestImpl:290`), which `RangerDefaultRequestProcessor:131-138` can only fill from `RangerRolesUtil`'s maps — built solely from the downloaded document and left empty when it is `null` (`RangerRolesUtil.java:44-75`). The fallback rewrite of roles into users (`RangerDefaultPolicyEvaluator:1025`) reads the same empty map *and* is gated on `disableRoleResolution`, which defaults to `true`.
3. **Live corroboration.** The coordinator's own log carries `PolicyEngineOptions: { ... disableRoleResolution: true ... }` and, every 30 seconds for the life of the pod, `Error getting Roles; service not found. ... response=404, lastKnownRoleVersion=-1`. `lastKnownRoleVersion` never leaves `-1`: the plugin has never held a role document. That is the plugin's own state, not an inference from a deny.

**The failure direction — this is the part that changes D1.** Ranger has no set-difference; a policy item matches only when its principal set *intersects* the request's, and the role clause short-circuits on an empty set (`RangerDefaultPolicyItemEvaluator:131-132`). An unmatched item is not neutral, it means whatever its bucket means: in `policyItems` it grants nobody, in `denyPolicyItems` it denies nobody, and **in `denyExceptions` it excepts nobody, so the deny stands for everyone** — the discard happens only when some exception evaluator returns `isMatch == true` (`RangerDefaultPolicyEvaluator:1331-1361`). A clearance written as *deny `{public}` except role `analyst`* would therefore refuse the tagged column to every user **including the cleared ones**. That is fail-closed on data and fail-open on availability: nothing leaks, and the control over-blocks loudly instead of silently doing nothing.

Two adjacent facts were checked because either could have made the deny shape inert on its own, and both came back favourable: **deny and exceptions are evaluated** despite egret's service-defs carrying `"options": null`, since `ServiceDefUtil.getOption_enableDenyAndExceptionsInPolicies:84-96` defaults the option to `true`; and **`{public}` in a deny item still matches everybody** with no group provider, because the group check needs only a non-null `userGroups` and `setUserGroups` normalises `null` to an empty set. The "everyone" half of the pair is expressible; only the "except these" half is not.

**Therefore: clearance compiles to the role's members enumerated at compile time**, as bare usernames in `denyExceptions`, and the Ranger `roles` bucket is not used. D3's third trigger — role membership changing — is no longer conditional; it is required.

**A latent SP4a defect this surfaced, to be raised separately rather than absorbed here.** `egret-perm-compile/src/grants.rs:105` already compiles a `PrincipalKind::Role` grant straight into the Ranger `roles` bucket, so that grant matches nobody in the engine — while egret's own evaluator matches it (`egret-perm/src/eval.rs:160`) and `explain` will report an access the engine does not grant. Same class of divergence D7 found for groups, on a path D7 did not look at.

**Scope of the measurement.** Trino only — **StarRocks is now measured separately in D0b below**, which found the deny shape works there, the role is no better, and a clearance can withhold reads only. The **Trino** plugin's on-disk role cache was still not inspected (it needs an `exec` into `demo-trino`, which is off limits); the mechanism makes it irrelevant, since a cache can only replay a download that succeeded and none ever did — and D0b read the StarRocks equivalent, which is an empty roles document, confirming exactly that.

## D0b — MEASURED: the deny shape DOES work on StarRocks, the role is no better there, and a clearance can only withhold READS

**Measured 2026-09-28. Full working: `docs/en/plans/2026-09-28-sp4c-d0b-starrocks-result.md`.** D0's method, which is D7's: upstream source first — StarRocks `148a7fdc44e` (`fe/fe-core/.../authorization/ranger/`, the engine's **own** plugin, not Trino's) and Apache Ranger `0274a6a0b`, the same tree D0 read — corroborated by passive reads of the **live** FE `sp3c2-sr-live-fe-0` on `kind-egret`: its `fe.conf`, its rendered `ranger-starrocks-security.xml`, its `fe.log`, and the plugin's own on-disk policy and roles caches. No query was run, nothing was modified, `demo-trino` untouched. D0 was explicitly Trino-only and nothing in it carried over.

1. **A resource `deny` with `denyExceptions` works.** `RangerStarRocksAccessController` is not a bespoke checker: every check funnels through `RangerAccessController.hasPermission` → `rangerPlugin.isAccessAllowed(request)` → `policyEngine.evaluatePolicies(request, POLICY_TYPE_ACCESS, null)` (`RangerBasePlugin.java:665-675`), i.e. the same `RangerDefaultPolicyEvaluator` / `RangerDefaultPolicyItemEvaluator` chain D0 read. The enabling option is `true` here too, and this was checked against the def the **live plugin holds** rather than only against egret's source: `ServiceDefUtil.getOption_enableDenyAndExceptionsInPolicies:84-96` has no per-engine carve-out, the cached `serviceDef` for `starrocks` (id 205) carries **no `options` key at all**, and egret's generated `ranger-starrocks-security.xml` sets none of the hidden properties. The deny half is **already proven live**: egret compiles resource denies to StarRocks today (two sit in the live plugin's cache) and SP4a's gate recorded the enforcement verbatim (`2026-09-26-starrocks-live-result.md` §6). `denyExceptions` specifically has **never been served to StarRocks** — none of the 32 cached policies carries one — so that half rests on source, and D5's live gate must observe it.
2. **The role is no better, and not for want of the data.** `RangerStarRocksAccessRequest.createAccessRequest` never calls `setUserRoles` — `grep -rn "setUserRoles" fe/` returns nothing at all, so StarRocks does not even pass Trino's explicit `null`. The session's native roles exist and are deliberately withheld: `ConnectContext.getCurrentRoleIds()` is read a few lines from the column check for the **error message** (`ColumnPrivilege.java:145`,`:157`) and **never** inside the `ranger` package. The downloaded document 404s exactly as on Trino — live, from `fe.log`: `Error getting Roles; service not found. ... response=404, serviceName=sp4a-sr-crosscheck, lastKnownRoleVersion=-1`, every 30s for the life of the FE. **D0's one open item is closed here**, by reading the file D0 could not reach: `/tmp/ranger/<svc>/policycache/starrocks_<svc>_roles.json` is `{"serviceName":…,"roleVersion":-1,…}` with no `rangerRoles` member, and `RangerRolesUtil`'s constructor builds its maps only inside `isNotEmpty(roles.getRangerRoles())`. `disableRoleResolution` still defaults `true` and egret sets it nowhere.
3. **An unresolvable principal denies EVERYBODY** — re-derived, not inherited, and it lands in the same place because the deciding code is shared and the StarRocks plugin adds nothing to it. So `denyExceptions` must carry **enumerated bare usernames**, as on Trino; StarRocks presents a bare username (SP4a measured `select current_user()` → `'srgate'@'%'`).

**What is genuinely different, and what it costs.**

- **No tag service, so no tag-first short-circuit.** A clearance on StarRocks is an ordinary resource policy competing with ordinary resource grants. It still wins, by a different mechanism: `PolicyEvalOrderComparator.compareNormal` sorts a `hasDeny()` policy first at equal priority, and an allow becomes final only once `!evaluator.hasDeny()` (`RangerPolicyEngineImpl.java:718-722`). **Caveat:** priority dominates that comparator, so a hand-written OVERRIDE-priority allow would beat a clearance deny. egret compiles everything at priority 0 (`is_override: false`), and D4's stated blindness to hand-written policy already covers this.
- **Column granularity works.** `checkColumnAction` builds a catalog/database/table/**column** resource, and `ColumnPrivilege.check:92-161` runs it **per column over the optimiser's pruned scan set** for every table whose catalog controller is an `ExternalAccessController`. A clearance therefore fires only when the classified column is actually read.
- **A clearance can withhold READS and nothing else.** `checkColumnAction` has exactly one caller and it passes `PrivilegeType.SELECT` (`ColumnPrivilege.java:152`); egret's `starrocks.json` independently restricts the `column` resource to `accessTypeRestrictions: ["select"]`. D1 withholds `select, insert, delete` on Trino; for StarRocks `engine_accesses` yields `select, insert, update, delete` and **three of the four are unreachable at column granularity**. Task 2d emits `select` alone and reports the gap; whether the write half should be bought with a *table*-level deny — which over-blocks every untagged column of that table — is a **user ruling**, not a compiler decision.
- **A view is not covered.** `ColumnPrivilege.check` routes a `View` to `checkViewPrivilege` and a materialized view to `checkMaterializedViewAction`, neither of which descends to base-table columns. So a clearance on a base-table column is not consulted when the data is read through a view. This is a property of the engine; it was **not** measured live, and D2's UI sentence must say it.

**Consequences.** Task 2d stays "expand per column" — one access policy per tagged column, the same body D1 builds with the tag resource replaced by the column resource SP4a's tag-mask expansion already computes. No dedup (unlike a row filter, an access deny *does* carry a column resource, so the count is exactly N) and the same refuse-not-truncate ceilings. D3's membership trigger covers this branch unchanged.

## D1 — Clearance compiles to a tag-based deny, which amends SP4a's D2

SP4a's D2 says **tags drive rules only, never access**. Clearance is the exception, and this spec amends D2 rather than quietly contradicting it: a classification whose whole purpose is to withhold data cannot be expressed as a mask or a row filter, because both still answer the query.

Clearance compiles to a **deny on the tag service** for the tagged resource, with the cleared principals in `denyExceptions`. Ranger's four-way allow / deny / allowExceptions / denyExceptions shape exists precisely because there is no principal set-difference, and "everyone except the cleared" is the case it was built for.

Two properties of Ranger make this the right layer, and both were established during SP4a:
- **Tag policies are evaluated first and short-circuit**, so a clearance denial wins over any resource grant. That is what clearance must do.
- **The tag service is the compiler's**, since SP4a's Important 5 claimed it — a hand-written tag ALLOW there could otherwise short-circuit ahead of a compiled deny.

## D2 — `required_groups` cannot be enforced at the engine, and the UI must say so

D7's measurement already settles half of this: a group named in a Ranger policy matches nobody. So a tag gated **only** by `required_groups` cannot be enforced at the data plane today — and D0 has now measured that a role name is no better, so what the UI must say is narrower than "roles work, groups do not": neither principal *name* reaches the engine, and what is enforced is the member set egret enumerated at compile time.

That is not a reason to silently skip it. The classification UI must say, where the clearance is set, that a group-only gate is enforced on object access and **not** in the engine — the same shape as SP4b's group warning, and for the same measured reason. A control that appears to withhold data while withholding nothing is the failure this programme keeps refusing to ship.

## D3 — Three new recompile triggers, because clearance has three inputs

A compiled clearance deny depends on the tag's gate, on which columns carry the tag, and on who holds the clearing role. So it goes stale from three directions:
- a tag's `required_roles` / `required_groups` changing — **new**
- a tag assignment changing — **exists** (SP4a Task 7)
- membership of a clearing role changing — **new**, and **required**: D0 measured that a role name is unresolvable in the engine, so the members are enumerated at compile time and go stale when membership changes

Each must recompile in the same transaction as its write, using the seam SP4a established: affected services expanded through `services_and_tag_services_via`, locked ascending, with the `debug_assert!(service_ids.is_sorted())` gate. SP4a shipped the "a write without its recompile" defect on four separate paths, so the enumeration of writers comes before the fix, not after.

## D4 — `explain` must name a clearance denial as its own reason

`explain` today would answer `no_grant_matched` for a subject blocked by clearance, which is the wrong administrative answer: nobody needs to add a grant, somebody needs clearance. It gains a reason class of its own, naming the tag.

This also narrows an existing limit. `explain` is blind to hand-written policy and will stay so in this spec — but it must not be blind to a mechanism this spec itself adds.

## D5 — The intersection property is the acceptance test

The point of the whole spec is one sentence: **a user without clearance cannot read a tagged column, by any path.** The test is therefore not that a policy was written, but that the read fails — through SQL Lab and through a notebook, since the notebook path is what made the union visible in the first place.

Live verification is required and is not satisfied by unit tests. SP4a shipped three Criticals that ~200 green unit tests passed over, each found only by running a real engine, and each of the same shape: a policy that compiled, deployed, and enforced nothing.

## Deliberately not in SP4c

- **Migrating hand-written policies** — now SP4d. The survey found structural blockers, not just volume: the delegator policies (`impersonate` on `trinouser`, `alter` on `systemproperty`) are inexpressible in a model scoped to catalog/schema/table/column, and `delegateAdmin`, override priority and `additionalResources` are all hardcoded off in the compiler. It also needs `explain` to stop being blind to hand-written policy first, or the migration cannot be previewed.
- **The per-workspace notebook role** — now SP4e, and it needs a correction first: the F2 design says notebook authz is keyed on Keycloak groups with Ranger deferred, and **that is not what runs**. Ranger is live, Trino connects with `X-Trino-User` keyed on the bare username, and workspace membership compiles to enumerated per-user grants. Building a notebook role on the group-keyed premise would build on a model that does not exist.
- Reconciling Cedar's object-level forbid with the data plane in the **other** direction — making a data-plane deny visible to Cedar. Out of scope; clearance flows one way here.
