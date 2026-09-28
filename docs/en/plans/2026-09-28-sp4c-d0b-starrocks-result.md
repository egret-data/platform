# D0b result — does a clearance deny work on StarRocks

**Measured:** 2026-09-28, on branch `feat/sp4c-classification-clearance`.
**Method:** D0's method, which is D7's. Upstream source first — StarRocks
`148a7fdc44e` (`fe/fe-core/.../authorization/ranger/`, the engine's **own**
Ranger plugin) and Apache Ranger `3.0.0-SNAPSHOT` at `0274a6a0b` (the same tree
D0 read) — corroborated by passive reads of the **live** StarRocks FE
`sp3c2-sr-live-fe-0` on `kind-egret`: its `fe.conf`, its rendered
`ranger-starrocks-security.xml`, its `fe.log`, and the plugin's own on-disk
policy and roles caches. **No query was run and nothing on the cluster was
modified.** `demo-trino` was not touched.

**Why this exists:** D0 (`2026-09-28-sp4c-d0-result.md`) was explicitly
Trino-only and said so in its own §5. StarRocks has a **different** Ranger
plugin, no tag service, and native SQL roles, so none of D0's three answers
could be carried over. SP4a only ever compiled *masks* to StarRocks, so the
**deny shape itself** was unproven on this engine.

---

## Answers, in one place

| question | answer |
|---|---|
| 1. Does a resource `deny` + `denyExceptions` work in the StarRocks plugin? | **Yes.** Same shared Ranger evaluator, the enabling option defaults `true` for the def the live plugin actually holds, and the deny half is already measured live. Column granularity works and covers **`select` only**. |
| 2. Does the plugin resolve a role name? | **No.** It never calls `setUserRoles`, it never forwards `ConnectContext.getCurrentRoleIds()`, and the downloaded roles document 404s. StarRocks' native roles are *available in the session* and *deliberately not given to Ranger*. |
| 3. Which way does an unresolvable principal fail? | **Deny everybody** — identical to Trino, and for the same shared reason. |

**So Task 2d has a mechanism.** It is not "there is no mechanism". But the
mechanism is narrower than Trino's in one way that needs a ruling: on StarRocks a
clearance can withhold **reads only**.

---

## 1. The deny shape works, and the deny half is already proven live

### 1a. The plugin runs the ordinary Ranger ACCESS evaluation

`RangerStarRocksAccessController` is not a bespoke allow-only checker. Every
check funnels through one method
(`fe/fe-core/src/main/java/com/starrocks/authorization/ranger/RangerAccessController.java:112-134`):

```java
RangerStarRocksAccessRequest request =
        RangerStarRocksAccessRequest.createAccessRequest(resource, user, groups, accessType);
RangerAccessResult result = rangerPlugin.isAccessAllowed(request);
if (result == null || !result.getIsAllowed()) {
    throw new AccessDeniedException();
}
```

`RangerBasePlugin.isAccessAllowed` (Ranger
`agents-common/.../service/RangerBasePlugin.java:665-675`) calls
`policyEngine.evaluatePolicies(request, RangerPolicy.POLICY_TYPE_ACCESS, null)`.
That is the same `RangerPolicyEngineImpl` → `RangerDefaultPolicyEvaluator` →
`RangerDefaultPolicyItemEvaluator` chain D0 read for Trino, in the same tree.
`denyPolicyItems` and `denyExceptions` are therefore evaluated by exactly the
code D0 quoted — `getMatchingPolicyItem`
(`RangerDefaultPolicyEvaluator.java:1331-1361`) discards a matched deny only if
some exception evaluator returns `isMatch(request) == true`.

The same source read also confirms the prior finding that **`root` bypasses the
check before Ranger is consulted** (`RangerAccessController.java:115-117`,
`if (UserIdentity.ROOT.equals(user)) return;`) — so no measurement may use
`root`.

### 1b. The enabling option is `true` for the def the live plugin actually holds

D0 established for Trino that
`ServiceDefUtil.getOption_enableDenyAndExceptionsInPolicies` defaults `true`
even with `options: null`. That function is **service-type agnostic** — read in
full it has no per-engine carve-out (`ServiceDefUtil.java:84-96`):

```java
boolean enableDenyAndExceptionsInPoliciesHiddenOption =
        config == null || config.getBoolean("ranger.servicedef.enableDenyAndExceptionsInPolicies", true);
boolean defaultValue = enableDenyAndExceptionsInPoliciesHiddenOption
        || StringUtils.equalsIgnoreCase(serviceDef.getName(), EmbeddedServiceDefsUtil.EMBEDDED_SERVICEDEF_TAG_NAME);
ret = ServiceDefUtil.getBooleanValue(serviceDef.getOptions(), OPTION_ENABLE_DENY_AND_EXCEPTIONS_IN_POLICIES, defaultValue);
```

and `getBooleanValue` (`:279-291`) returns `defaultValue` unchanged when the map
is null or empty. Both inputs were checked on the **live** engine rather than
only in egret's source:

- egret's `crates/egret-policy/src/service-defs/starrocks.json` has **no
  `options` key at all** (D0's phrase "options: null" — same effect,
  `getOptions()` is null). Confirmed against the document the running plugin
  holds: `/tmp/ranger/sp4a-sr-crosscheck/policycache/starrocks_sp4a-sr-crosscheck.json`
  parses to `serviceDef.name = starrocks`, `serviceDef.id = 205`, and
  `'options' in serviceDef == False`.
- The hidden property is not set. egret's generated
  `ranger-starrocks-security.xml`, read verbatim from
  `/opt/starrocks/fe/conf/` on the live FE, carries exactly seven properties
  (service name, rest url, source impl, cache dir, poll interval, two timeouts)
  and nothing else. Upstream's own `conf/ranger/ranger-starrocks-security.xml`
  sets no policy-engine options either.

So `denyEvaluators` and `denyExceptionEvaluators` are built, not
`Collections.emptyList()` (`RangerDefaultPolicyEvaluator.java:130-138`).

### 1c. The deny half is already measured live on this engine

egret **already** compiles resource-level denies to StarRocks, and the live
plugin is holding two of them right now. The cached policy document on
`sp3c2-sr-live-fe-0` has 32 policies (22 access, 6 mask, 4 row-filter), of which
two carry non-empty `denyPolicyItems`, e.g. verbatim:

```json
{"id": 8796, "name": "perm-grant-c4dddd35-…", "policyType": 0,
 "resources": {"catalog": {"values": ["sp4a10_wh"]}, "database": {"values": ["sp4a"]},
               "table": {"values": ["secret"]}, "column": {"values": ["*"]}},
 "denyPolicyItems": [{"accesses": [{"type": "select", "isAllowed": true}],
                      "users": ["srgate"], "delegateAdmin": false}]}
```

and SP4a's own live gate (`2026-09-26-starrocks-live-result.md` §6, case S2)
recorded the enforcement verbatim:

```text
SELECT id,note FROM sp4a10_wh.sp4a.secret ORDER BY id
ERROR 5204 (42000) at line 1: Access denied; you need (at least one of) the SELECT privilege(s)
                              on COLUMN note for this operation.
```

with the sibling table still returning rows in the same session. **A resource
deny is enforced on StarRocks and is scoped to what it names.**

`denyExceptions` specifically has **never been served to StarRocks** — none of
the 32 cached policies carries one. Its evaluation rests on the source read in
§1a, not on a live observation. Task 6's live gate must observe it.

### 1d. Deny beats allow across policies — by mechanism, not only by S2

This matters because on Trino a clearance is a **tag** policy and D1 relies on
Ranger's tag-first short-circuit to beat any resource grant. StarRocks has **no
tag service** (`tagPolicies: null` in the cached document; `tagService` names no
row for this profile), so a clearance there is an ordinary resource policy
competing with ordinary resource grants. It still wins:

- `RangerPolicyEvaluator.PolicyEvalOrderComparator.compareNormal`
  (`RangerPolicyEvaluator.java:260-274`) sorts a policy that `hasDeny()` **before**
  one that does not, at equal priority.
- `RangerPolicyEngineImpl.evaluatePoliciesForOneAccessTypeNoAudit`
  (`RangerPolicyEngineImpl.java:718-722`) makes an allow final only once no deny
  policies are left:

  ```java
  if (ret.getIsAllowed()) {
      if (!evaluator.hasDeny()) { // No more deny policies left
          ret.setIsAccessDetermined(true);
      }
  }
  ```

**One caveat, from the same loop.** Priority dominates the comparator
(`Integer.compare(other.getPolicyPriority(), me.getPolicyPriority())`), and an
allow decided by a **higher-priority** policy is final
(`if (ret.getPolicyPriority() > evaluator.getPolicyPriority())`). egret compiles
everything at normal priority — D1 sets `is_override: false`, and the live cache
shows `"policyPriority": 0` — so a hand-written OVERRIDE-priority allow would
beat a clearance deny. `explain` is already documented as blind to hand-written
policy; this is the same blindness, not a new one.

### 1e. Column granularity works, and covers `select` only

The deny has to land on a column, because a clearance is per classified column.
It does: `RangerStarRocksAccessController.checkColumnAction`
(`starrocks/RangerStarRocksAccessController.java:159-170`) builds a
catalog/database/table/**column** resource and goes through the same
`hasPermission`. And that check really runs on reads —
`ColumnPrivilege.check` (`authorization/ColumnPrivilege.java:92-161`) marks
every table whose catalog controller `instanceof ExternalAccessController` (the
Ranger controller is one) and, for those, checks **per column** over the
optimiser's *pruned* scan set:

```java
Set<String> columns = scanColumns.getOrDefault(tableName, new HashSet<>());
for (String column : columns) {
    Authorizer.checkColumnAction(context, tableName, column, PrivilegeType.SELECT);
}
```

Pruning is a feature here, not a hazard: a clearance fires only when the
classified column is actually read.

**But `checkColumnAction` has exactly one caller and it passes
`PrivilegeType.SELECT`** (grep over `fe/fe-core/src/main/java/`:
`ColumnPrivilege.java:152` is the only call site). Independently, egret's own
`starrocks.json` restricts the `column` resource to
`accessTypeRestrictions: ["select"]`, while `insert`/`update`/`delete`/`export`
are restricted to `table`. So:

> **On StarRocks a clearance can withhold READS and nothing else.** D1 withholds
> `select, insert, delete` on Trino (`withheld_accesses` × `engine_accesses`,
> which for StarRocks yields `select, insert, update, delete`). Three of those
> four are unreachable at column granularity: no column-level write check is
> ever issued, so such an item would be dead weight, and putting them on a
> *table*-level deny instead would withhold the whole table rather than the
> classified column.

(For completeness: `accessTypeRestrictions` is admin-side only — the only plugin
reader is `RangerBaseService.java:201-204`, not the evaluator — so egret *could*
emit `insert` on a column resource. It would simply never match. The
restriction is not what blocks the write half; the absent call site is.)

**Two read paths a column deny does not cover**, both from the same function:
a **view** is checked by `checkViewPrivilege` and a **materialized view** by
`checkMaterializedViewAction` (`ColumnPrivilege.java:136-147`), neither of which
descends to the base table's columns. A clearance on a base-table column is
therefore not consulted when the data is read through a view. This is a property
of the engine, not of egret's compilation, and it was **not** measured live.

## 2. The plugin does not resolve a role — and it is not for want of the data

This is the question that could have made StarRocks *better* than Trino. It does
not.

1. **The plugin never sets `userRoles`.**
   `RangerStarRocksAccessRequest.createAccessRequest`
   (`ranger/RangerStarRocksAccessRequest.java:32-47`) sets user, groups, access
   type, resource, client IP, client type, cluster name and access time — and
   **no roles**. Trino's plugin at least passes an explicit hardcoded `null`;
   StarRocks does not mention roles at all. `grep -rn "setUserRoles" fe/`
   returns **nothing** across the whole front end. The field then defaults to
   what `RangerAccessRequestImpl` leaves it at, and every write path normalises
   `null` to an empty set (`RangerAccessRequestImpl.java:290-292`) — never a
   wildcard.
2. **The native role ids are right there and are not forwarded.** StarRocks
   knows the session's activated roles: `ConnectContext.getCurrentRoleIds()` is
   used a few lines away from the column check, in
   `AccessDeniedException.reportAccessDenied(...)`
   (`ColumnPrivilege.java:145`, `:157`). `grep -rn "getCurrentRoleIds"
   fe/fe-core/src/main/java/com/starrocks/authorization/ranger/` returns
   **nothing**: the whole Ranger package never reads it. The engine's own roles
   are used for the *error message* and not for the *decision*. So "resolve from
   the engine's own catalog" is not merely unconfigured — there is no code for
   it.
3. **The downloaded roles document 404s, exactly as on Trino.**
   `RangerBasePlugin.init()` starts a `PolicyRefresher` that owns a
   `RangerRolesProvider` unconditionally (`PolicyRefresher.java:52`, `:86`,
   `:131`, `:198`, `:530-534`), and egret serves no roles route. **Live, from
   `fe.log` on `sp3c2-sr-live-fe-0`** — the StarRocks plugin's own state, not an
   inference from a denial:

   ```text
   2026-09-28 00:03:42.236Z ERROR (PolicyRefresher(serviceName=sp4a-sr-crosscheck)-65|65)
     [RangerAdminRESTClient.getRolesIfUpdated():255] Error getting Roles; service not found.
     secureMode=false, response=404, serviceName=sp4a-sr-crosscheck,
     lastKnownRoleVersion=-1, lastActivationTimeInMillis=1790468384343
   2026-09-28 00:03:42.351Z WARN  … [getRolesIfUpdated():263] Received 404 error code with body:[null], Ignoring
   ```

   It repeats every 30s for the life of the FE and `lastKnownRoleVersion` never
   leaves `-1`.
4. **D0's one open item is now CLOSED, for StarRocks, by reading the file.** D0
   could not inspect the plugin's on-disk role cache because that needed an
   `exec` into `demo-trino`. `sp3c2-sr-live` is under no such constraint, and
   the cache exists and is empty:

   ```text
   /tmp/ranger/sp4a-sr-crosscheck/policycache/starrocks_sp4a-sr-crosscheck_roles.json
   {"serviceName":"sp4a-sr-crosscheck","roleVersion":-1,"roleUpdateTime":"20260927-00:19:43.922-+0000"}
   ```

   No `rangerRoles` member at all. `RangerRolesUtil`'s constructor builds its
   maps only inside `if (CollectionUtils.isNotEmpty(roles.getRangerRoles()))`
   (`RangerRolesUtil.java:43-72`), so every map stays empty, and
   `RangerAuthContext.getRolesForUserAndGroups` (`:104-129`) returns nothing
   from either the user half or the group half. The cache cannot rescue a role
   name; it only ever replays a download that never succeeded.
5. **Secondary role resolution is off by default and egret does not turn it on.**
   `RangerPolicyEngineOptions.disableRoleResolution` is initialised `true` at the
   field (`:44`) and re-read as `conf.getBoolean(prefix +
   ".policyengine.option.disable.role.resolution", true)` (`:102`, `:123`).
   Neither egret's generated `ranger-starrocks-security.xml` nor upstream's
   sample sets it.

**A note on groups, which differ from Trino.** StarRocks *does* plumb groups —
`createAccessRequest` passes `ConnectContext.getGroups()`, written by
`AuthenticationHandler`, `ConnectProcessor` and `ExecuteAsExecutor`. That does
**not** make a group-gated clearance enforceable, because egret pushes no group
membership of any kind (D7's third finding, unchanged) and does not configure a
StarRocks group provider. Whether a StarRocks group provider could be configured
to agree with egret's groups was **not measured**, and is out of D0b's scope.
What matters for the deny shape is only that `userGroups` is non-null, so
`public` (Ranger's `GROUP_PUBLIC`, the literal string) still matches: `matchUserGroupAndOwner` needs
`userGroups != null && policyItem.getGroups() != null` and then
`policyItem.getGroups().contains(GROUP_PUBLIC)`
(`RangerDefaultPolicyItemEvaluator.java:127-130`), and `setUserGroups`
normalises `null` to an empty set (`RangerAccessRequestImpl.java:294-296`).

## 3. An unresolvable principal denies EVERYBODY, exactly as on Trino

Not inherited — re-derived, and it lands in the same place because the deciding
code is shared and the StarRocks plugin adds nothing to it.

`matchUserGroupAndOwner` is pure set intersection, with the role clause guarded
on a non-empty request-side set (`RangerDefaultPolicyItemEvaluator.java:118-142`):

```java
if (!ret && CollectionUtils.isNotEmpty(roles) && CollectionUtils.isNotEmpty(policyItem.getRoles())) {
    ret = !Collections.disjoint(policyItem.getRoles(), roles);
}
```

With `roles` empty — §2 — a role-only item never matches. And an unmatched item
is not neutral; what it means is decided by its bucket:

| bucket | principal unresolvable ⇒ | net effect |
|---|---|---|
| `policyItems` (allow) | never matches | grants nobody |
| `denyPolicyItems` | never matches | denies nobody |
| `denyExceptions` | never matches | **the deny stands for everyone** |

So *deny `public` / except role `analyst`* would refuse the classified column
to every StarRocks user including the cleared ones. Fail-closed on data,
fail-open on availability, loud rather than silent — the same verdict as D0, on a
separate plugin.

The corollary is the same too: the exception must name **bare usernames**.
Enumeration works on StarRocks because Ranger matches `policyItem.getUsers()` by
string (`:124-126`), and SP4a's live gate already measured that StarRocks
presents a bare username (`select current_user()` → `'srgate'@'%'`).

## 4. What this means for Task 2d

1. **There IS a mechanism. Task 2d stays "expand per column".** One access
   policy per tagged column, `policyType 0`, resources
   `catalog/database/table/column`, `denyPolicyItems: [{groups: ["public"]}]`,
   `denyExceptions: [{users: [<enumerated members>]}]` — the same body D1 builds,
   with the tag resource replaced by the column resource SP4a's tag-mask
   expansion already computes (`egret-perm-compile/src/rules.rs`,
   `column_resources`).
2. **The withheld access list must be narrowed to `select` on this branch.**
   D1's `withheld_accesses` yields `select, insert, update, delete` for
   StarRocks. Only `select` is reachable at column granularity. Emitting the
   other three writes items that can never match — the "control that appears to
   withhold and withholds nothing" this programme keeps refusing to ship — so
   Task 2d should emit `select` alone **and report the gap**, and the user must
   rule on whether the write half is wanted at table granularity (which
   over-blocks the untagged columns) or not at all.
3. **The expansion is per column, and dedup does not apply.** SP4a's tag *mask*
   dedups per column and its row *filter* dedups per table because a row-filter
   policy has no column resource. An access deny **does** carry a column
   resource, so every tagged column is its own policy and the count is exactly
   N. It competes for the same per-rule and profile-total ceilings, and must
   refuse rather than truncate, naming the profile and the running total.
4. **Ordering is safe without a tag service.** Do not assume the tag-first
   short-circuit D1 relies on; it does not exist here. Deny-beats-allow comes
   from the evaluator sort plus the `!evaluator.hasDeny()` gate (§1d), which
   holds only at equal policy priority — so Task 2d must keep
   `is_override: false`/priority 0 consistent with the grants it must beat, and
   must not introduce an override-priority allow anywhere.
5. **Trigger 3 is mandatory here too.** Same enumeration, same staleness, same
   recompile-on-role-membership-change requirement as Trino (SP4c Task 3).
6. **Task 5's UI warning gains a second sentence for StarRocks:** a clearance
   withholds reads and not writes there, and is not consulted when the column is
   read through a view.

## 5. What this measurement does NOT establish

- **`denyExceptions` has never been observed enforcing on StarRocks.** The deny
  half is measured live (§1c); the exception half rests on the shared-source
  read (§1a). Task 6's live gate must run the cleared/uncleared pair on a
  StarRocks engine, not only on Trino.
- **Views and materialized views were not measured.** Source says the column
  check is not reached through them (§1e); no query confirmed it.
- **Whether a StarRocks group provider could make a group-gated clearance
  enforceable.** Not examined. egret configures none and pushes no group
  membership, so today the group-only skip D1 already reports is correct on this
  engine as well.
- **Nothing was measured about the write half in the engine.** The claim is that
  no column-level write check is *issued* (single call site, `SELECT`), which is
  a source fact. Whether a table-level write deny behaves as expected was not
  tested.
- **No query was run at all.** By design: the deny that matters was already
  measured live by SP4a's gate, and D0b's new claims are the ones a denied query
  could not have settled anyway.
